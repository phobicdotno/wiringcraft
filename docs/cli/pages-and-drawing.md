# Pages and drawing from the command line

These commands make a project, manage its pages and draw on them: symbols, black boxes and
wires. Apart from `new`, they work on the project in the current folder; give another one with
`-C <dir>` (or `--project <dir>`) before the command, e.g. `wiringcraft -C pump place ...`.
Positions and sizes are in millimetres. `wiringcraft <command> --help` shows every option.

The commands change the project's files but do not commit them; use `wiringcraft vcs` for that.

## new

```sh
wiringcraft new pump --name "Pump station"
cd pump
```

Creates the folder `pump` with a project in it and makes it a git repository with a first
commit (`--no-git` skips that). The name defaults to the folder's name. A new project has one
page, `p0001`, number 1, marked as the cover page.

`--template <folder or git URL>` starts from a base project instead. Everything in it is copied
except its review comments, its history and symbolic links: pages, settings, title block, library sources,
pictures, macros (`macros/`), device data (`devices.json`) and the vendored library items
(`vendor/`). `{name}` and `{date}` in its title block and page titles become the new project's
name and today's date.

A new project has no library sources, so `place` finds no symbols until one is added, e.g.
`wiringcraft lib add builtin folder <path to the library folder>`.

## page

Pages are named by their id (`p2`). The commands below that take `<ID>` also take the page
number. Only `page add` takes the id alone, and `place`, `box`, `box-edit`, `wire` and
`delete` take the page id, not the number.

| Command | What it does |
|---|---|
| `page add <ID> --number <N>` | Add a page. `--title`, `--size` (default `A3`), `--overview`, `--cover` |
| `page list` | List the pages: id, number, title, symbol and wire counts, and the `cover` or `overview` mark |
| `page edit <ID>` | Change the `--number` and `--title`. A number another page has is refused |
| `page size <ID> <SIZE>` | Change the sheet size. What is drawn stays where it is |
| `page overview <ID>` | Mark it as an overview (line diagram); `--off` makes it a normal page |
| `page cover <ID>` | Make it the cover page (any other page loses the mark); `--off` takes the mark off |
| `page fields <ID> [key=value]...` | List the page's title-block fields, or set the page's own values |
| `page remove <ID>` | Delete the page and everything drawn on it |

A size is `A4`, `A3`, `A2`, `A1` or `A0`, landscape unless followed by `-portrait`
(`A4-portrait`), or `WIDTHxHEIGHT` in millimetres (`500x700`). Pages are kept sorted by number.

```sh
wiringcraft page add p2 --number 2 --title "Control"
wiringcraft page add p3 --number 3 --title "Spare" --size 500x700
wiringcraft page edit 3 --number 2.1
wiringcraft page list
```

```
p0001      1      Cover (0 symbols, 0 wires), cover
p2         2      Control (0 symbols, 0 wires)
p3         2.1    Spare (0 symbols, 0 wires)
```

`page fields` sets the title-block fields it lists (`revision`, `date`, `drawn_by`,
`checked_by`). A page's own value wins over the project's on that page; `key=` clears it, so the
project's value shows again.

```sh
wiringcraft page fields p2 revision=B drawn_by=JS
wiringcraft page fields p2 revision=
```

## place

```sh
wiringcraft place p2 wc.button.no --at 50,40 --tag -S1
wiringcraft place p2 wc.coil --at 50,80 --tag -K1
wiringcraft place p2 wc.contact.no --at 90,40 --tag -K1 --rotate 90 --mirror
```

Places a library symbol (by its id, as `lib search` lists it) with its origin at `--at` and
prints its item id (`s1`, `s2`, ...). `--tag` takes a tag that starts with `-`. `--rotate` is
0, 90, 180 or 270; `--mirror` flips it left to right before turning it. Symbols that share a
tag are one device (here the coil and the contact of `-K1`).

## box

```sh
wiringcraft box p2 --at 150,40 --size 40,30 --tag -A1 \
  --pin L:left:10 --pin N:left:20 --pin OUT:right:15
```

Draws a black box for a device without a symbol: a rectangle with its top-left corner at
`--at`, `--size` wide and high, and prints its item id. Each `--pin name:side:offset` puts a
connection point on the `left`, `right`, `top` or `bottom` edge, `offset` mm from that edge's
left or top end. Wires attach to the points like symbol pins. The box is stored as a symbol of
the project (`box.<page>.<id>`), so it has a device tag and part like any symbol.

## box-edit

```sh
wiringcraft box-edit p2 s4 --size 50,30
wiringcraft box-edit p2 s4 --rename L=L1
wiringcraft box-edit p2 s4 --pin L1:left:10 --pin PE:bottom:25
wiringcraft box-edit p2 s4 --clear-pins
```

Changes a box drawn with `box`, named by page id and item id. Sizes, sides and offsets are in
the box's own frame, and its own top-left corner stays put.

- `--size w,h` alone keeps the points, pulled onto the edges of the new size.
- `--pin` replaces every point: a point not named again is taken off, and a wire on it keeps a
  free end where it was (a warning says so).
- `--rename OLD=NEW` renames a point and its wires follow. For a name with `=` in it, use
  `--rename-pair OLD NEW`.
- `--clear-pins` takes every point off.

Every box placing the same symbol changes.

## wire

```sh
wiringcraft wire p2 s1:14 s2:A1 --potential 24V
wiringcraft wire p2 50,20 s1:13 --potential 24V --section "0.75 mm²" --colour BU
wiringcraft wire p2 s2:A2 90,100 --via 50,100
wiringcraft wire p2 s3:14 120,40 --potential 24V
```

Draws a wire and prints its id (`w1`, ...). Each end is `item:pin` (a symbol's or a box's
point) or a point `x,y` on the page. `--via x,y` adds a bend point; repeat it for more. The
wire's data: `--number`, `--potential`, `--section` (with its unit), `--colour`, `--type`
(e.g. `H07V-K`), and `--cable` with `--core` when the wire is a core of a cable.

## delete

```sh
wiringcraft delete p2 s3 w2
```

```
s3
w2
w4
```

Deletes symbols and wires by id and prints each deleted id. Wires that end on a pin of a
deleted symbol go too (here `w4`, which ended on `s3`). The device keeps its data. An id that
is not on the page is an error, and then nothing is deleted.

## number-wires

```sh
wiringcraft number-wires
wiringcraft number-wires --rule sequential --overwrite
```

```
numbered 2 connection(s); 2 wire(s) changed
```

Numbers the wires of the whole project. Every wire of one connection gets the same number.
`--rule` is one of:

- `page-column` (default): the page and column where the connection starts, `2.3`; more
  connections starting in the same column get `2.3.2`, `2.3.3`.
- `sequential`: 1, 2, 3 in drawing order.
- `potential`: the potential's name (`24V`), else sequential. A connection with no potential, or
  with a name another connection already has, gets the next free sequential number.

A connection that already has a number keeps it unless `--overwrite` is given. Wires that touch
no pin, and the wires of overview pages, are left alone. `report wires` lists the result.

## find

```sh
wiringcraft find 24v
wiringcraft find -K1 --replace -K2
```

```
2	Potential	24V
2	Device tag	-K1
replaced 1 value(s)
```

Finds text in free texts, device tags, wire numbers, potentials and device descriptions on
every page and prints one hit per line: page number, kind, value (tab-separated). Case does not
matter unless `--match-case`. A query or replacement that starts with `-` is taken as text.
`--replace` replaces the text in every hit; replacing a device tag renames the device
everywhere. Status lines (`replaced ...`, `nothing found`) go to stderr, so stdout holds only
hits. Finding nothing is not an error.
