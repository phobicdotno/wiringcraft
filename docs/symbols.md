# Symbols

A symbol is the reusable drawing of one device function: a relay coil, a normally open
contact, a 3-phase motor. You place symbols on pages and join their pins with wires. This page
explains how symbols work. The exact JSON layout of a symbol file is in
[library-format.md](library-format.md).

## What a symbol is made of

Each symbol is one JSON file named after its id, for example `wc.contact.no.json`. It holds:

- **Id.** A stable name such as `wc.contact.no`. Pages refer to symbols by id, so an id is never
  reused for something else. Ids may use `A-Z a-z 0-9 . _ -` (up to 128 characters, not
  starting with a dot).
- **Name, category, description.** What the library list and search show (`Normally open
  contact`, category `contacts`).
- **Letter code.** Suggested for new devices placed with this symbol, for example `K` for
  relays and contactors.
- **Graphics.** Lines, polylines, rectangles, circles, arcs and texts, in mm around the
  symbol's origin (0,0). Texts such as the `13` and `14` printed by a contact are part of the
  graphics.
- **Pins.** The connection points. Each pin has a name (`A1`, `13`, `L1`), a position and the
  direction a wire leaves it (up, down, left or right). Wires attach to pins by name, as
  `s1:13`.
- **Tag position.** Where the device tag (`-K1`) is printed.
- **Role.** What the symbol does for cross-references: `main` (a coil that drives contacts
  with the same tag), `auxiliary` (a contact driven by that coil), `terminal` (a terminal of a
  strip), `interruption` (the wire goes on at every other interruption point with the same
  tag) or `standalone` (everything else).

A symbol has no maker data. Order numbers, ratings and terminals belong to parts, which a
device links to separately (`wiringcraft device`).

## The starter library

WiringCraft ships a starter library in the `library/` folder: about 75 symbols named `wc.*`
(contacts, coils, buttons, breakers, fuses, motors, sensors, terminals, PLC points and more)
and a few example parts. It is a plain library folder with `symbols/` and `parts/` inside.

A project made in the app gets the starter library as its source, when the app finds a
`library/` folder next to itself (or in the installed package). A project made with
`wiringcraft new` starts with no library source, so add one yourself:

```
wiringcraft new myproj --no-git
cd myproj
wiringcraft lib add starter folder /path/to/wiringcraft/library
wiringcraft lib search coil
```

`lib search` finds symbols and parts by id, name, category, letter code or description.

A project can list several sources (local or network folders, or git repositories). When two
sources hold the same id, the one listed first wins.

## The project's own copies

A project never draws from the library directly. The first time you place a symbol, WiringCraft
copies it into the project's `vendor/symbols/` folder and records its source and a content hash
in `library.lock`:

```
wiringcraft page add p1 --number 2 --title Power
wiringcraft place p1 wc.coil --at 50,50 --tag=-K1
```

After that the project uses its copy. It opens and prints the same even when the library
changes, moves or is offline. `wiringcraft lib status` shows each pinned symbol as up to date,
changed in the library (with the differences), or gone from the library (the project copy keeps
working).

To take newer versions, run `wiringcraft lib update` (all changed items) or
`wiringcraft lib update wc.coil` (only that one). An update replaces the project's copy, so a
change made by hand in `vendor/symbols/` is lost. To keep such a change, offer it back to a
library with `wiringcraft lib propose <source> <id>`: the file goes to
`<source>/proposed/symbols/<id>.json` for someone to review.

`wiringcraft lib lock` pins symbols that are used but not pinned yet (for example after a hand
edit of a page file) and writes a pinned copy again when it is missing.

## Placing, turning and mirroring

A placed symbol has a position, a turn and a mirror flag. The symbol's origin lands on the
position you give.

- **Turn** is 0, 90, 180 or 270 degrees, nothing in between. In the app, press `R` or use
  Rotate on the selection or on the symbol you are about to place. On the command line, use
  `--rotate`.
- **Mirror** flips the symbol left to right. In the app, press `M` or use Mirror. On the command
  line, use `--mirror`. The mirror is applied before the turn.

Pins and wire directions turn and mirror with the drawing. Texts inside the symbol keep their
reading direction so they stay readable.

```
wiringcraft place p1 wc.contact.no --at 100,50 --tag=-K1 --rotate 90 --mirror
```

`place` prints the new symbol's id on the page (`s1`, `s2`, ...). Use it to wire the symbol
(`wiringcraft wire p1 s1:A2 s2:13`) or to delete it.

A device that has no symbol of its own can be drawn as a black box: a rectangle with named
connection points on its edges (`wiringcraft box`, or Black box in the app). It is stored as a
symbol of the project and carries a tag and a part like any other.

## Making or editing a symbol

**In the app.** Library > New symbol opens the symbol editor. Draw lines, rectangles, circles,
polylines, arcs and texts, place and name the pins, set the tag position, and save into one of
the project's library folders. Everything snaps to a 2.5 mm grid so pins line up with wires.
Library > Edit symbol opens an existing library symbol (the selected one, or one you pick).

When you save a symbol the project already uses, the app updates the project's copy and the
pages that use it. It keeps the old version and says so when wires end on pins the new version
no longer has, or when another library listed earlier has a symbol with the same id.

**From other tools.** Library > Import symbols in the app, or the `import` commands, turn
drawings and other libraries into symbols in a library folder (the `--lib` option):

- `import symbol-svg` and `import symbol-dxf`: one symbol from an SVG or DXF drawing, with pins
  marked in the drawing (see `--help` for how).
- `import symbol-kicad`: every symbol of a KiCad symbol library.
- `import symbol-qet`: QElectroTech elements.
- `import parts-edz` and `import swe-library`: symbols that come with Eplan or SolidWorks
  Electrical part data. Without `--lib`, `swe-library` writes into the project's own copies.

After saving or importing into a library, run `wiringcraft lib update <id>` in projects that
already use that id, so they take the new version.

**By hand.** A symbol file is plain JSON, so you can also write or edit one in a text editor.
Edit the library's file, not the project's copy in `vendor/`, and follow
[library-format.md](library-format.md).
