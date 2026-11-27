# Reports

WiringCraft builds its reports from the schematic each time you ask for one. They are never
stored in the project, so they cannot be out of date. The same reports show in the desktop app,
in `wiringcraft report` and on the report pages of a PDF. The commands and their options are in
[docs/cli/reports-and-export.md](cli/reports-and-export.md).

This page lists each report with its columns in their built-in order. A project can pick,
reorder, rename and sort the columns of a report with `wiringcraft report-layout` (`[[report]]` in
`project.toml`); the column names below are the ones a layout refers to.

Overview pages (line diagrams) draw the schematic's connections again, so the wire, net, cable,
terminal and PLC reports skip them. Tags are sorted with numbers by value (`-K2` before `-K10`).

## The reports

`all` gives them in this order.

### contents: Table of contents

Columns: `Page`, `Title`, `Devices`, `Wires`.

One row per page, cover and overview pages included. `Devices` counts the different tags drawn on
the page, leaving out interruption points. In a PDF it is printed before the schematic pages.

### parts: Parts list

Columns: `Pos`, `Qty`, `Manufacturer`, `Order number`, `Description`, `Devices`.

One row per part, with how many are used and by which devices. A device's accessories count with
their quantity, a jumper with a part counts once under its strip, and a strip device drawn as
terminals tagged with only the strip (`-X2`) counts its part once per terminal. A part missing
from the project's library copy shows its id as the order number and says so in `Description`.
Devices with no part come last, with an empty `Pos`, `Qty` 1 and `no part assigned`.

### devices: Device list

Columns: `Device`, `Description`, `Part`, `Symbols`, `Pages`.

Every device: tags drawn on pages, tags only in `devices.json`, and cables named only on wire
cores. A tag used only by interruption points is left out. `Part` is the part id. A cable's
`Pages` include the pages its cores are drawn on.

### wires: Wire list

Columns: `Page`, `Wire`, `Potential`, `Cable`, `From`, `To`, `Section`, `Colour`, `Type`.

One row per drawn wire. `Potential` is the wire's own potential, else its net's name (a generated
one is `N1`, `N2`, ...). `Cable` is `<cable>:<core>`. A free wire end shows as
`junction <x>,<y>`, and a pin on an untagged symbol as `page <n> <item>:<pin>`.

### nets: Net list

Columns: `Net`, `Pins`, `Connections`.

One row per connected group of pins across pages. `Net` is its potential name (several are joined
by ` / `) or a generated `N<n>`. `Pins` is how many pins it has and `Connections` names them.
Interruption points are not listed as pins.

### terminals: Terminal plan

Columns: `Strip`, `Terminal`, `Upper (pin 1)`, `Lower (pin 2)`, `Potential`, `Page`,
`Jumpered with`, `Level`.

One row per drawn terminal, strip by strip. `-X1.3` is terminal 3 of strip `-X1`; terminals
tagged with just the strip are numbered in drawing order. The upper and lower columns show what
is wired to that side of the terminal itself, while `Potential` also follows jumpers. `Level` is
filled for multi-level terminals only. When the strip editor (`wiringcraft strip`) has set the
rail order, the plan follows it, and each spare terminal gets a row with `spare` in `Page`. A
terminal whose symbol has no copy in the project shows `(symbol <id> missing)` in the upper column.

### cables: Cable list

Columns: `Cable`, `Type`, `Description`, `Core`, `Page`, `From`, `To`.

One row per core. `Cable`, `Type` (manufacturer and order number of its part) and `Description`
are filled on the first core's row only, unless a report layout sorts or groups the list. A cable
in `devices.json` with no cores drawn yet gets one row with empty core cells.

### plc: PLC I/O list

Columns: `Address`, `Type`, `Device`, `Description`, `Connected to`, `Page`.

Every drawn PLC input or output: a symbol in the `plc` category whose name says input or output,
or whose only pin is `I` or `Q`. `Type` is `Input` or `Output`. `Address` is the device property
`Address` (or `PLC address`); points without one come last. A point on a symbol with no copy in
the project is left out, and the command warns about the missing symbol.

### plc-cards: PLC cards

Columns: `Rack`, `Slot`, `Card`, `Type`, `Channel`, `Address`, `Device`, `Description`,
`Connected to`, `Page`.

Every card of every rack (`wiringcraft rack`), slot by slot and channel by channel, with the PLC
point drawn at each channel's address. A channel with nothing drawn has empty point cells (a
spare); two points on one address give two rows. A card whose channels cannot be worked out is
left out, and the rack warnings say why. In a PDF each card is also drawn as a sheet of its own.

### locations: Locations

Columns: `Location`, `Description`, `Devices`, `Note`.

The project's locations (`wiringcraft location`) with how many devices are in each, counted by
the `+` part of their tags (a device in `+L1.L3` counts for `+L1.L3`, not `+L1`). Locations that
tags use but the project does not list come after, with `not in the project's locations` in
`Note`. A listed path that is not a valid location keeps its row, with the reason in `Note`.

### networks: Networks

Columns: `Network`, `Type`, `Device`, `Description`, `Address`, `Station`, `Port`,
`Connected to`, `Cable`, `Pages`.

One row per port link of each device on a fieldbus network (`[[network]]` in `project.toml`),
in the order they are written there. A device with no links gets one row with empty port cells.
In a PDF the list is followed by a topology page per network.

## Empty reports

A report with nothing to list still has its title and column headings:

- `report <kind>` prints the heading lines only (text and Markdown), or the header row only (CSV).
- An Excel workbook from `report all --format xlsx` has a sheet for every report, also the empty
  ones, and `report all --format csv --out <folder>` writes all eleven files.
- On a PDF report page an empty table says `Nothing to list.`

One exception: `pdf --report all` leaves out the PLC cards report when the project has no racks,
and the networks report when it has no networks, so the PDF does not gain an empty page. Naming
`plc-cards` or `networks` with `--report` prints them anyway.

## Example

On the motor-starter project that `examples/demo.sh` builds, which has a cable but no PLC:

```
$ wiringcraft report cables
Cable list
Cable  Type  Description             Core  Page  From               To
-----  ----  ----------------------  ----  ----  -----------------  -----------------
-W1          Lamp cable to the pump  1     3     =PUMP+CAB1-X1.1:2  =PUMP+CAB1-P1:X1
                                     2     3     =PUMP+CAB1-P1:X2   =PUMP+CAB1-X1.2:1

$ wiringcraft report plc --format md
## PLC I/O list

| Address | Type | Device | Description | Connected to | Page |
|---|---|---|---|---|---|
```
