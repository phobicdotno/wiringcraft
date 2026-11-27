# PLC inputs, outputs and racks

This page explains how WiringCraft keeps track of PLC inputs and outputs: the points you draw,
their addresses, the racks and cards they sit on, and the lists and files made from them.

## PLC points on the drawings

A PLC input or output is a symbol whose library category is `plc` (or whose id contains
`.plc.`), named as an input or an output. The bundled library has `wc.plc.input` and
`wc.plc.output`. Place one per channel and give it a device tag, as for any other symbol.

The point's address is a property of its device: `Address` (`PLC address` is read too). It
lives in `devices.json` with the device's description:

```json
"-B1": {
  "description": "Start button",
  "properties": { "Address": "%IX0.0" }
}
```

You can set it in two ways:

- In the app, select the symbol and type `Address` and the value under Device properties.
- From a list: `wiringcraft import plc-csv <file>` (File > PLC addresses in the app).

Addresses are read in the common spellings: `%IX0.0`, `%I0.0`, `I0.0`, `E0.0` for inputs,
`%QX0.0`, `Q0.0`, `A0.0` for outputs, and byte, word and double word addresses such as `%IB4`,
`%IW64` and `%QD8`. Spaces and case do not matter.

## Importing an address list

`import plc-csv` takes a CSV file or an Excel workbook (`.xlsx`, first visible sheet). It needs
a header row with a device column and an address column; a description column is optional.
Column names are matched in several languages, for example `Device`, `Tag` or `BMK` for the
device and `Address`, `PLC address`, `IO address` or `Operand` for the address.

Each row sets that device's address, and its description when the row has one. An empty address
clears it. A tag that is neither drawn nor already in the device list (`devices.json`) is reported and
skipped, not added. A device in the list that no symbol carries yet is updated like a drawn one.

```
$ wiringcraft import plc-csv io.csv
PLC addresses: 3 device(s) changed, 0 already up to date, 0 unknown
```

`wiringcraft report plc --format csv` (or `--format xlsx --out <file>`) writes a list that reads
back with `import plc-csv`, so the list can go to a spreadsheet and back.

## The PLC I/O list

`wiringcraft report plc` lists every drawn PLC point by address: address, input or output,
device, description, what it is wired to and the page. Points without an address come last, so
the gaps are easy to see. Overview pages are skipped, because they draw devices a second time.

## Racks and cards

Racks describe the PLC hardware: a rack (or station) with an I/O card in each slot. They are
kept in `project.toml` as `[[rack]]` tables, each with its `[[rack.card]]`s:

```toml
[[rack]]
name = "-A1"
description = "Main PLC"

[[rack.card]]
slot = 1
tag = "-A1.DI1"
type = "DI 8x24 V DC"
channels = 8
start = "%IX0.0"
```

A card has a slot, an optional device tag and type, a number of channels and the address of
channel 0 (`start`). The start address says whether the card has inputs or outputs and how the
channels are numbered:

- A bit address (`%IX0.0`) counts channels bit by bit: `%IX0.0` to `%IX0.7`, then `%IX1.0`.
- A byte, word or double word address (`%IB4`, `%IW64`, `%QD8`) moves 1, 2 or 4 bytes per
  channel. `step` replaces that distance, for PLCs that number words one by one (`step = 1`
  gives `%IW0`, `%IW1`).

Limits: slots 0 to 999, at most 256 channels, a step from 1 to 64.

Edit racks in the app under File > PLC racks, or on the command line:

```
$ wiringcraft rack add -A1 --description "Main PLC"
$ wiringcraft rack card add -A1 1 --tag -A1.DI1 --type "DI 8x24 V DC" --channels 8 --start %IX0.0
$ wiringcraft rack card add -A1 2 --type "DO 8x24 V DC" --channels 8 --start %QX0.0
$ wiringcraft rack list
-A1 (Main PLC)
  slot 1: -A1.DI1 "DI 8x24 V DC" 8 channels from %IX0.0
  slot 2: "DO 8x24 V DC" 8 channels from %QX0.0
```

`rack set` renames a rack or changes its description, `rack card set` changes a card (options
left out are kept) and `rack remove` and `rack card remove` take them out. Run
`wiringcraft rack --help` for the options.

## The PLC card overview

The `plc-cards` report lays out every card channel by channel, with the point drawn at each
channel's address. A channel with nothing drawn is listed empty (a spare). Print it with
`wiringcraft report plc-cards`, or add it to the PDF with `wiringcraft pdf out.pdf --report
plc-cards`. In the PDF it comes as a table and as one drawn sheet per card: the card as a box
with a terminal per channel, its address and the device and page of each point.
`--report all` leaves the overview out of a project that has no racks.

## Rack warnings

The overview also checks the racks against the drawings and warns about:

- a card whose channels cannot be worked out (a start that is not a PLC address, no channels),
- two cards in one slot, or two cards sharing an address (a word covers two bytes, so `%IW64`
  and `%IB65` overlap),
- a drawn point that is on no card: its address is on none, is not a PLC address, is in the
  other direction, or names bit 8 or higher.

Points without an address are not warned about here; the PLC I/O list shows them last. The
warnings are printed after every `rack` change and by `report` and `pdf`, for example:

```
warning: PLC input -B9 (page 2) at %IX4.0 is on no card of the PLC racks
```

The app shows them in the PLC racks form while you type. `wiringcraft check` does not list them.

## Variable list for the PLC program

`wiringcraft plc-vars <file>` writes the PLC points as an IEC 61131-3 global variable list in
structured text, for TwinCAT, CODESYS and other tools that read it:

```
$ wiringcraft plc-vars GVL_IO.st
wrote GVL_IO.st: 4 variable(s)
```

Part of the file:

```
VAR_GLOBAL
    B1 AT %IX0.0 : BOOL; (* -B1, Start button, page 2 *)
    K1 AT %QX0.0 : BOOL; (* -K1, Pump contactor, page 2 *)
END_VAR
```

There is one variable per point, named after its device tag (`-A1.DI1` becomes `A1_DI1`; a name
that would start with a digit or is a keyword gets `io_` in front, and a name used twice gets
`_2`). Its type follows the address size: `BOOL`, `BYTE`, `WORD` or `DWORD`. A point without
an address, or with one that is not a PLC address, is declared at `%I*` or `%Q*` so it can be
linked in the PLC tool, and the command warns how many there are. The comment holds the tag,
description, what it is wired to and the page.

## AutomationML

`wiringcraft aml <file>` (the AutomationML export in the app's File menu) writes the devices,
their part data, properties and connection points, and the connections between them, as
AutomationML (CAEX 3.0).
A PLC point's `Address` goes along as a device attribute. The racks and cards are not part of
this export. `wiringcraft import aml` reads such a file back as overview pages.
