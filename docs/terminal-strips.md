# Terminal strips

This page explains how WiringCraft thinks about terminal strips: which symbols are terminals,
how they get their numbers, the order on the rail, spare terminals, multi-level terminals,
jumpers, and how the terminal plan report is put together. The commands themselves are listed
in [docs/cli/devices-and-terminals.md](cli/devices-and-terminals.md).

## Terminals and strips

A terminal is a placed symbol with the terminal role, such as `wc.terminal`. Its strip and its
number come from its device tag: `-X1.3` is terminal `3` of strip `-X1`, and `=A+C1-X1.3` is
terminal `3` of `=A+C1-X1`. The number is always one tag level, so `1.2` cannot be a terminal
number. A symbol the project has no copy of still counts as a
terminal when its tag has a terminal level (`-X1.3`).

A terminal tagged with only its strip (`-X1`) gets a number of its own: the next one after the
highest number drawn on that strip, in drawing order (page by page, then left to right). With
`-X1.1`, `-X1.2` and `-X1.10` drawn, two more terminals tagged `-X1` become `11` and `12`.
Numbers taken by spare terminals are passed over.

Terminals on an overview page (a line diagram) are drawn again there but not counted again: the
terminal plan and the strip editor list each terminal once, from the schematic pages.

## Rail order

By default a strip's terminals are in number order. Numbers sort by their value, so `2` comes
before `10`, and strips sort the same way (`-X2` before `-X10`).

The terminal strip editor (Terminal strips in the app, `wiringcraft strip` on the command line)
can put the terminals in another order on the rail. That order is kept in a `[[strip]]` table in
`project.toml`. Terminals drawn after the order was set follow at the end in number order. Going
back to an empty order returns the strip to number order.

The terminal plan, the WAGO marking export and the WAGO configurator export all follow the rail
order. See [wago-marking.md](wago-marking.md) and [wago-configurator.md](wago-configurator.md).

## Spare terminals

A spare terminal is on the rail but not drawn on any page, for example a reserve terminal
`S1`. It is stored in the strip's `[[strip]]` table and has a place in the rail order like any
other terminal. In the terminal plan it gets a row of its own with `spare` in the Page column
and nothing wired to it. If a terminal with the spare's number is drawn later, the spare is
that drawn terminal.

## Multi-level terminals

A multi-level terminal is one terminal block with two or more levels stacked on each other.
WiringCraft models each level as its own terminal number with a `Level` property on that
terminal's device (`-X1.11` in `devices.json`), where 1 is the bottom level.

You only need to set the upper levels. A terminal with no `Level` of its own, right below a
terminal at level 2 or more, is shown as level 1: setting `-X1.11` to level 2 makes `-X1.10`
level 1, and the two are one two-level block. "Right below" means the next number down in
number order, not on the rail, so moving terminals on the rail never changes their levels. A
spare only counts here when it has a level of its own.

A `Level` value that is not a whole number from 1 up is ignored: the terminal counts as
single-level, and printing the terminal plan (`report terminals` or `pdf --report terminals`)
warns about it.

## Jumpers

A jumper (bridge) joins two or more terminals of one strip. Jumpered terminals are one
potential: wires on `-X1.1` and wires on `-X1.2` are in the same net once the two are
jumpered, across pages too. Jumpers are kept in `[[jumper]]` tables in `project.toml`, each
with its strip and terminal numbers.

- A jumper always holds at least two different terminals of one strip.
- Adding terminals that share a terminal with an existing jumper makes one bigger jumper.
- Taking a terminal out of a jumper that then has only one left removes the jumper.
- A jumper can include spare terminals. Removing a spare takes it out of its jumper too.
- A jumper can have a part (article), such as a WAGO 2002-402 2-way jumper. Jumpers with a
  part count once each in the parts list, under their strip, and go into the WAGO
  configurator export.

On the schematic pages and in the PDF a jumper is drawn as a bar through its terminals on that
page, in number order, with a dot on each. When terminals are renamed, renumbered or moved, the
jumper follows them; a deleted terminal leaves its jumper. `wiringcraft check` reports a jumper
that names a terminal drawn nowhere.

## How the terminal plan is built

The terminal plan is the `terminals` report: `wiringcraft report terminals` prints it, and
`wiringcraft pdf out.pdf --report terminals` adds it to the PDF as table pages. It is built like
this:

1. Every terminal on the schematic pages is found and given its strip and number as above.
2. Strips are listed in value order. A strip that only has spares is listed too.
3. Within a strip, terminals come in rail order, with spares in their places.
4. For each terminal the plan shows:
   - **Upper (pin 1)** and **Lower (pin 2)**: what is wired to that side of this terminal
     itself, without the jumpers.
   - **Potential**: the potential name of the terminal's net, with the jumpers. A spare
     jumpered to a drawn terminal shows that terminal's potential.
   - **Page**: the page the terminal is drawn on, or `spare`.
   - **Jumpered with**: the other terminals of its jumper.
   - **Level**: its level on a multi-level terminal, empty for a single-level terminal.

The columns, sorting and grouping of the report can be changed with a report layout
(`wiringcraft report-layout`).

## Example

Three drawn terminals, a lamp wired to `-X1.1`, and two terminals tagged with just `-X1`:

```sh
wiringcraft place p2 wc.terminal --at 40,60 --tag "-X1.1"
wiringcraft place p2 wc.terminal --at 60,60 --tag "-X1.2"
wiringcraft place p2 wc.terminal --at 80,60 --tag "-X1"
wiringcraft place p2 wc.terminal --at 100,60 --tag "-X1.10"
wiringcraft place p2 wc.terminal --at 120,60 --tag "-X1"
wiringcraft place p2 wc.lamp --at 40,120 --tag -P1
wiringcraft wire p2 s1:2 s6:X1
wiringcraft jumper add -X1 1 2
wiringcraft strip spare -X1 S1 --after 2
wiringcraft strip move -X1 12 1
wiringcraft strip level -X1 11 2
wiringcraft strip list
```

```text
-X1 12 1 2 S1 (spare) 10 11
```

```sh
wiringcraft report terminals
```

```text
Terminal plan
Strip  Terminal  Upper (pin 1)  Lower (pin 2)  Potential  Page   Jumpered with  Level
-----  --------  -------------  -------------  ---------  -----  -------------  -----
-X1    12                                                 2
-X1    1                        -P1:X1         N1         2      2
-X1    2                                                  2      1
-X1    S1                                                 spare
-X1    10                                                 2                     1
-X1    11                                                 2                     2
```

The two `-X1` terminals became `11` and `12`. Terminal `10` shows level 1 because `11` is
level 2. Terminal `2` has no wire of its own, so it shows no potential yet; once a wire is on
it, it shows `N1`, the same potential as terminal `1`.
