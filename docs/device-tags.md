# Device tags

Every device in a WiringCraft project is named by a device tag (a reference designation in
the IEC 81346 style), such as `=PUMP+CAB1-K1`. The tag is how symbols, device data, reports,
cross-references, panel layouts and jumpers find each other.

## The three parts

A tag has up to three parts, each starting with its own sign:

| Sign | Part | Meaning | Example |
|------|------|---------|---------|
| `=` | Function | The plant section or function the device belongs to | `=PUMP` |
| `+` | Location | Where it sits: cabinet, room, panel | `+CAB1` |
| `-` | Product (device id) | The device itself | `-K1` |

- The product part is required. Function and location are optional, so `-K1`, `+CAB1-K1` and
  `=PUMP+CAB1-K1` are all valid.
- Each part can have several levels separated by `.`, for example `=PUMPS.P1+CAB1.R2-K3`.
- Only letters, digits, `.` and `_` are allowed inside a part. No spaces.
- Each part may appear once. `-K1-K2` is refused.
- When saved, the parts are always written in the order `=`, `+`, `-`.

The leading letters of the product part are the letter code: `K` for `-K12`, `XD` for `-XD3`.
The letter code is what the next free tag is counted from (see below).

Tags that differ in any part are different devices. `-K3` and `+CAB1-K3` are two devices.

A bad tag is refused with a message that names the problem:

```
$ wiringcraft place p1 wc.coil --at 60,140 --tag K2
error: invalid device tag "K2": must start with =, + or -
$ wiringcraft place p1 wc.coil --at 60,140 --tag "-K 2"
error: invalid device tag "-K 2": only letters, digits, '.', '_' allowed
```

### Terminals

A terminal's tag is its strip plus a number as the last product level: `-X1.3` is terminal
`3` of strip `-X1`. A terminal tagged with the strip only (`-X1`) gets its number from the
drawing order.

### Locations

`wiringcraft location set +CAB1 "Main cabinet"` gives a location a description. The list of
locations is only for descriptions and the `locations` report. A tag can use a location that
is not in the list, and `location remove` leaves device tags as they are.

## One tag, one device

All symbols with the same tag are one device. A relay coil `=PUMP+CAB1-K1` and its contacts
tagged `=PUMP+CAB1-K1` on any page are the same relay, so the PDF draws cross-references
between them. The device's data (description, part) is stored once per tag in `devices.json`.

`wiringcraft check` reports a device drawn twice: one tag on two coils, or on two copies of
the same standalone symbol or terminal. Contacts may repeat. Overview pages do not count.

## How the next free tag is picked

The CLI never picks a tag for you. `wiringcraft place` without `--tag` places an untagged
symbol, and an untagged symbol does not show in the device list.

The app picks tags in two ways:

- **Placing a symbol from the library.** The symbol's letter code (for example `K` for a
  coil, `X` for a terminal) is numbered from 1 up, and the first number that no symbol on any
  page uses is taken, so a gap left by a deleted device is filled first. The number is counted
  on the product part alone: `-K1` in `+CAB2` also blocks `-K1` in `+CAB1`. Function and
  location are taken from the tags already on the page (the most common pair), so the new
  device lands in the same section and cabinet as its neighbours. A symbol without a letter
  code is placed untagged.
- **Copy and paste, Duplicate (Ctrl+D), duplicating a page and placing a macro.** Each device
  gets the first free tag with the same function, location and letter code as its old one,
  numbered 1 to 9999. Here a tag is free when no symbol and no device entry uses the full tag.
  Symbols that shared a tag in the copy share the new tag too. The device data goes with the
  copy. When no tag is left, a paste places the symbols untagged, and a macro is not placed at
  all (`no free tag like ...`).

## How tags show on pages and in reports

On a page the tag is drawn next to the symbol, one part per line (`=PUMP`, `+CAB1`, `-K1`),
centred on the symbol's tag position.

On a panel layout a device is labelled with its product part only (`-K1`), unless another
device on the same plate has the same product part (`+CAB1-K1` and `+CAB2-K1`); then the full
tag is shown.

Reports list devices by their full tag:

```
$ wiringcraft report devices
Device list
Device         Description     Part  Symbols                            Pages
-------------  --------------  ----  ---------------------------------  -----
=PUMP+CAB1-K1  Pump contactor        Normally open contact, Relay coil  2
```

In CSV output a tag is written with a leading `'` (for example `'-K3`), so a spreadsheet does
not read a tag that starts with `-`, `+` or `=` as a formula.

The `locations` report counts the devices per location.

## Renaming a device

To rename a device everywhere, use find and replace: Ctrl+F in the app, or `find --replace`
on the command line. Replacing a device tag renames the device entry, every symbol with the
tag, the cable cores of a cable, panel items, terminal-strip jumpers and fieldbus networks.

```
$ wiringcraft find =PUMP+CAB1-K1 --replace =PUMP+CAB1-K5
2	Device tag	=PUMP+CAB1-K1
2	Device tag	=PUMP+CAB1-K1
replaced 2 value(s)
```

Things to know:

- Find matches text inside a tag, so `find -K1 --replace -K2` also turns `-K15` into `-K25`.
  Give the full tag to rename one device only.
- A rename onto a tag another device already uses is refused (`... is already used.`), and so
  is a replace that would give two devices the same tag. Nothing changes then.
- A replace that renames `-K1` to `-K2` and `-K2` to `-K3` in one go works.

Changing the tag in a symbol's properties in the app is different: it retags only that one
symbol. The symbol joins the device with the new tag (or starts a new one), and the old
device keeps its data and its other symbols.

Tags given on the command line start with `-`, so the CLI takes them as values (`--tag -K1`,
`device -K1`). After such a command, write the project folder as `-C <dir>`, not `-C<dir>`,
since `-C1` is a tag too.
