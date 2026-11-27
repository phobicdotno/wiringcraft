# Cables

A cable in WiringCraft is an ordinary device with a tag such as `-W1` or `=PUMP+CAB1-W1`.
It has no symbol of its own. Wires on the schematic say which cable they run in and which core
of it they are. The cable list, the wire list and the check read the cable from those wires.

## Cable tags

Give a cable a device tag like any other device. The letter code `W` is the usual one, and it
matters in one place: a cable that is listed in `devices.json` but has no cores drawn yet only
shows in the cable list when the last part of its tag starts with `W` (`-W5`, `-W`, `-WD3`).
A terminal named like a cable (`-X1.W1`) that sits on a terminal strip is not taken for a cable.

A cable that is named by wire cores is always a device, even with no symbol and no entry in
`devices.json` yet. It shows in the device list and the parts list like the rest.

Renaming a cable with `find --replace` (or the app's Find and replace window) changes the tag in
every core that names it.

## Cable types

The cable's type is its part, set the same way as for any device:

```
wiringcraft device -W1 --part demo.olflex-4g1-5 --description "Pump motor feed"
```

A cable part is a normal library part (`parts/<id>.json`): manufacturer, order number,
description and free properties. WiringCraft does not read the cores from the part, so the core
names on the wires are up to you.

`import swe-library` reads the cable types of a SolidWorks Electrical archive (`.tewzip`) as
parts. Each one gets the properties `Kind = Cable`, `Cores` (how many), `Core colours`
(`1 BN, 2 BU, 3 GNYE`, when the archive has colours) and `Core cross-section` (when all cores
have the same one).

## Assigning wires to cores

A wire becomes a core when it names a cable and, usually, a core. The core name is free text:
a number (`1`) or a colour (`BN`, `GNYE`).

On the command line, `wire` takes `--cable` and `--core`. The cable tag may start with `-`
without a `--` in front:

```
wiringcraft wire p1 s2:2 s1:U --cable -W1 --core 1
wiringcraft wire p1 s5:2 s1:PE --cable -W1 --core GNYE
```

`--core` without `--cable` is refused (`--core needs --cable`). `--cable` alone is allowed: the
wire then runs in the cable without naming a core.

In the app, select a wire and fill in Cable and Core in its properties. The small list next to
Cable offers the cables already used in the project. An empty Cable takes the wire out of its
cable. A tag that cannot be read shows an error under the field and nothing changes.

On the page, in the app and in the PDF, the core is written next to the wire as `-W1:1`
(or `-W1` when no core is named), after the wire number and the potential.

In the page file the wire stores it as:

```json
"cable": { "cable": "-W1", "core": "1" }
```

## The cable list

`report cables` prints one block per cable: tag, type, description, then one row per wire that
names a core of it, with the page and the two ends of that wire. A core drawn as two wires (two
wires naming `-W1:1`) has two rows, so count cores, not rows.

```
$ wiringcraft report cables
Cable list
Cable  Type            Description               Core  Page  From      To
-----  --------------  ------------------------  ----  ----  --------  ------
-W1    Demo CAB-4G1.5  Pump motor feed           1     2     -X1.1:2   -M1:U
                                                 2     2     -X1.2:2   -M1:V
                                                 3     2     -X1.3:2   -M1:W
                                                 GNYE  2     -X1.PE:2  -M1:PE
-W2                    Spare cable to the valve
```

How the columns are filled:

- Cables are sorted by number value (`-W2` before `-W10`), and cores the same way.
- Type is the part's manufacturer and order number. A part missing from the project's library
  copy is shown by its id.
- Description is the device's description, else the part's description.
- From and To are `tag:pin`. A wire end that is a free point shows as `junction x,y`.
- A cable from `devices.json` with no cores drawn yet (`-W2` above) gets one row with empty
  core cells.
- Wires on overview pages are left out, so a core drawn again on an overview is listed once.

The cable, type and description are named only on a cable's first row. When the report layout
sorts or groups the rows (`report-layout`), each row names its cable. In the PDF
(`pdf --report cables`) a cable whose cores run onto the next page is named again at the top of
that page, and each row links to the page its core is drawn on.

The same table comes out as CSV, Markdown or an Excel sheet with `--format csv|md|xlsx`. An Excel
sheet needs a file to go in: `wiringcraft report cables --format xlsx --out cables.xlsx`.

## Cables in other reports and exports

- The wire list (`report wires`) has a Cable column with `-W1:1` for each core.
- The device list and the parts list count the cable as a device with its part.
- `export wago-marking --kind cables` writes one marker row per cable, two copies each (one for
  each end).

## Checks

`wiringcraft check` (and the check list in the app) looks at cables in two ways:

- A core is one conductor. When wires naming the same cable and core sit on different
  connections, it warns: `cable core -W1:3 is on 2 connections`. Wires that touch no pin, wires
  on overview pages and wires that name no core are not counted.
- A check rule of kind `cable` warns about matching devices that have no cabled wire on any of
  their pins. For example, every motor needs a cable. `rules set` replaces all of the project's
own rules with the ones it reads, so on a project that has rules already, put its other
`[[rule]]` tables in the input too (`rules list` shows them):

```
printf '[[rule]]\nname = "motor-cable"\nkind = "cable"\ndevices = "-M*"\n' | wiringcraft rules set -
wiringcraft check
warning page 2: -M2 has no cable (check rule 1 "motor-cable")
```

## Not there yet

- The cores of a cable type are not checked against the core names used on the wires, and
  nothing lists a cable's unused cores.
