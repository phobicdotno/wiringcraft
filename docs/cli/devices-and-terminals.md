# Devices, locations, terminals and PLC on the command line

This page covers `device`, `location`, `jumper`, `strip`, `rack` and `plc-vars`. Each command
works on the project in the current folder; give `-C <dir>` (or `--project <dir>`) to name
another one. `wiringcraft <command> --help` lists the same options.

Device tags start with a dash, and these commands take a tag such as `-K1` as a value. Because
of that, write the project option with a space after a tag (`device -K1 -C ../plant`): an
attached `-C../plant` there is read as a tag.

The examples below were run in order on one scratch project, made like this:

```sh
wiringcraft new plant --no-git
cd plant
wiringcraft lib add std folder /path/to/wiringcraft/library
wiringcraft page add p1 --number 2 --title "Control"
for i in 1 2 3 4; do wiringcraft place p1 wc.terminal --at $((30+i*15)),60 --tag=-X1.$i; done
wiringcraft place p1 wc.coil --at 150,60 --tag=-K1
wiringcraft place p1 wc.plc.input --at 200,60 --tag=-B1
wiringcraft place p1 wc.plc.output --at 230,60 --tag=-P1
```

A terminal tagged `-X1.1` is terminal `1` of strip `-X1`.

## device

Set a device's part and description, or with neither option print them. The data goes to
`devices.json`.

| Option | Effect |
|---|---|
| `--part <id>` | the part's library id; a part the project does not have yet is copied in from the library sources. `--part ""` takes the part off |
| `--description <text>` | what the device does |

```sh
$ wiringcraft device -K1 --part example.ex-ct-9-24dc --description "Pump contactor"
$ wiringcraft device -K1
part         example.ex-ct-9-24dc
description  Pump contactor
$ wiringcraft device -K1 --part ""
```

A tag that is drawn nowhere is an error (`error: no device -K9`).

## location

The project's locations (cabinets, rooms, panels), the `+` part of device tags. They are kept
as `[[location]]` tables in `project.toml`. The `+` may be left out.

| Subcommand | Effect |
|---|---|
| `list` | list the locations with their descriptions |
| `set <path> [description]` | add a location or change its description; with no description the old one stays, `""` clears it |
| `remove <path>` | take a location off the list; devices keep their tags |

```sh
$ wiringcraft location set +L1 "Main cabinet"
$ wiringcraft location set L1.L3 "Cabinet 3"
$ wiringcraft location list
+L1	Main cabinet
+L1.L3	Cabinet 3
$ wiringcraft location remove +L1.L3
```

## jumper

Jumpers (bridges) between terminals of one strip. Jumpered terminals are one potential. They
are kept as `[[jumper]]` tables in `project.toml`.

| Subcommand | Effect |
|---|---|
| `add <strip> <terminals>...` | jumper at least two terminals; a jumper that shares a terminal with them is merged into one |
| `remove <strip> <terminals>...` | take terminals out of their jumper; a jumper left with one terminal goes |
| `part <strip> <terminal> [part]` | give the jumper holding that terminal a part (article); left out, the part is cleared |
| `list` | list the jumpers, with their parts in brackets |

```sh
$ wiringcraft jumper add -X1 1 2
$ wiringcraft jumper add -X1 2 3
$ wiringcraft jumper list
-X1 1 2 3
$ wiringcraft jumper part -X1 1 example.ex-tb-2-5
$ wiringcraft jumper list
-X1 1 2 3 (example.ex-tb-2-5)
$ wiringcraft jumper remove -X1 3
```

A terminal must be on the strip, drawn or spare: `jumper add -X1 9 1` gives
`error: -X1 has no terminal 9`.

## strip

The terminal strip editor: the order of a strip's terminals on the rail, spare terminals and
terminal levels. The terminal plan (`report terminals`) and the WAGO configurator export follow
it. Order and spares are kept as `[[strip]]` tables in `project.toml`; a level is the
terminal device's `Level` property in `devices.json`.

| Subcommand | Effect |
|---|---|
| `list [strip]` | list the strips with their terminals in rail order; spares are marked `(spare)` |
| `order <strip> [terminals]...` | put the terminals in this rail order; terminals left out follow at the end; no terminals goes back to the order by number |
| `move <strip> <terminal> <to>` | move a terminal to a place on the rail, 1 being first; a place past the end puts it last |
| `spare <strip> <terminal> [--after <terminal>]` | add a spare terminal (on the rail, not drawn); without `--after` it goes at the end |
| `unspare <strip> <terminal>` | take a spare off the strip, and out of its jumper |
| `level <strip> <terminal> [level]` | set a terminal's level on a multi-level terminal, 1 being the bottom; left out, the terminal's own `Level` is cleared |

```sh
$ wiringcraft strip order -X1 3 1 2
$ wiringcraft strip list -X1
-X1 3 1 2 4
$ wiringcraft strip move -X1 4 1
$ wiringcraft strip spare -X1 S1 --after 2
$ wiringcraft strip list -X1
-X1 4 3 1 2 S1 (spare)
$ wiringcraft jumper add -X1 S1 4
$ wiringcraft strip level -X1 4 2
$ wiringcraft strip order -X1
$ wiringcraft strip list -X1
-X1 1 2 3 4 S1 (spare)
```

Levels that are not set are inferred in number order, not rail order: a terminal whose next
number up has level 2 or more is still level 1.

## rack

PLC racks and the I/O cards in their slots, kept as `[[rack]]` tables in `project.toml`. The
PLC card overview (`pdf --report plc-cards`, `report plc-cards`) lays them out. After each
change the overview's rack warnings are printed on stderr, for example a PLC point whose
address is on no card.

| Subcommand | Effect |
|---|---|
| `list` | list the racks with their cards by slot: tag, type, channels, start address and step |
| `add <name> [--description <text>]` | add a rack; its name is usually the PLC's device tag |
| `set <name> [--name <new>] [--description <text>]` | rename a rack or change its description; an empty description clears it |
| `remove <name>` | remove a rack and its cards |
| `card add <rack> <slot>` | add a card to a free slot (0 to 999) |
| `card set <rack> <slot>` | change a card; options left out are kept |
| `card remove <rack> <slot>` | take a card out of its slot |

Card options:

| Option | Effect |
|---|---|
| `--tag <tag>` | the card's device tag, e.g. `-A1.DI1`; with `card set`, empty clears it |
| `--type <text>` | what the card is, e.g. `DI 16x24 V DC` or an order number; with `card set`, empty clears it |
| `--channels <n>` | how many channels, at most 256 (`card add` default: 16) |
| `--start <address>` | the address of channel 0, e.g. `%IX0.0` or `%IW64` |
| `--step <bytes>` | bytes from one channel to the next for byte, word and double word cards, 1 to 64; default the address's size |
| `--slot <slot>` | `card set` only: move the card to this slot |
| `--no-step` | `card set` only: go back to the address's own size as the step |

```sh
$ wiringcraft rack add -A1 --description "Main PLC"
$ wiringcraft rack card add -A1 1 --tag -A1.DI1 --type "DI 16x24 V DC" --channels 16 --start %IX0.0
$ wiringcraft rack card add -A1 2 --tag -A1.DO1 --type "DO 8x24 V DC" --channels 8 --start %QX0.0
$ wiringcraft rack card add -A1 3 --type "AI 4x" --channels 4 --start %IW64
$ wiringcraft rack card set -A1 3 --slot 4 --step 4
$ wiringcraft rack list
-A1 (Main PLC)
  slot 1: -A1.DI1 "DI 16x24 V DC" 16 channels from %IX0.0
  slot 2: -A1.DO1 "DO 8x24 V DC" 8 channels from %QX0.0
  slot 4: "AI 4x" 4 channels from %IW64 step 4
$ wiringcraft rack card remove -A1 4
$ wiringcraft rack set -A1 --name -A2 --description "Line PLC"
```

Renaming a rack does not change its cards' tags.

## plc-vars

Write the PLC inputs and outputs as an IEC 61131-3 global variable list (structured text) for
TwinCAT, CODESYS and similar tools. Each drawn PLC input or output becomes one variable, named
after its device tag and located at its address. The comment after it holds the tag, the
device's description and the page.

```sh
wiringcraft plc-vars GVL_IO.st
```

A point's address is its device's `Address` (or `PLC address`) property. One way to set them
is `import plc-csv` with a device and an address column:

```sh
$ printf 'Device,Address\n-B1,%%IX0.0\n-P1,%%QX0.0\n' > io.csv
$ wiringcraft import plc-csv io.csv
PLC addresses: 2 device(s) changed, 0 already up to date, 0 unknown
$ wiringcraft device -B1 --description "Tank full"
$ wiringcraft plc-vars GVL_IO.st
wrote GVL_IO.st: 2 variable(s)
$ cat GVL_IO.st
(* PLC inputs and outputs of plant, written by WiringCraft from the drawings. *)
VAR_GLOBAL
    B1 AT %IX0.0 : BOOL; (* -B1, Tank full, page 2 *)
    P1 AT %QX0.0 : BOOL; (* -P1, page 2 *)
END_VAR
```

A point with no address, or one that is not a PLC address, is declared at `%I*` or `%Q*`, to be
linked to the hardware in the PLC tool, and a warning says how many:
`warning: 2 point(s) have no PLC address and are declared at %I* or %Q*`. Addresses are read in
the common spellings (`%IX0.0`, `%I0.0`, `I0.0`, `E0.0`, `IW64`, `A4.1`). `report plc` lists the
same points as a table.
