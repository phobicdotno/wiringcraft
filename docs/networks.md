# Fieldbus networks and topology pages

A network in WiringCraft is a fieldbus such as PROFINET or EtherCAT: which devices are on
the bus, their bus addresses, and the cables between their ports. From it WiringCraft makes
two things:

- the Networks report, a table with one row per device port, and
- a topology page per network at the end of the PDF, numbered `N1`, `N2` and so on.

Networks are kept in `project.toml`. There is no network editor in the app and no CLI
command that edits them yet, so you write the tables by hand.

## Networks, nodes and links

Each network is a `[[network]]` table with its `[[network.node]]` and `[[network.link]]`
tables under it:

```toml
[[network]]
name = "PN1"
kind = "profinet"
description = "Line 1 field devices"

[[network.node]]
device = "-A1"
address = "192.168.0.1"
station = "plc-line1"

[[network.node]]
device = "-K10"
address = "192.168.0.10"

[[network.link]]
from = "-A1:X1P1"
to = "-K10:X1P1"
cable = "-W20"
```

- `name` names the network. Two networks with one name get a warning.
- `kind` is the bus type. It is free text: `profinet` (or `pn`) is shown as `PROFINET`,
  `ethercat` (or `ecat`) as `EtherCAT`, and anything else as written. It can be left out.
- `description` is optional and shows in the topology page title.
- A node is a device on the bus: its device tag, its `address` (an IP address on PROFINET,
  a station address on EtherCAT) and, if it has one, its `station` name (the PROFINET
  device name). Address and station are optional.
- A link joins two ports, each written `<device tag>:<port>`, such as `-A1:X1P1`. The port
  can be left out (`-K10`). `cable` is the cable's device tag and is optional.

Keys WiringCraft does not know are kept when the project is saved.

A device keeps its place on its networks when it is renamed: `find --replace` and the
assistant's rename change the node, the link ends (keeping the port) and the cable tag too.

## The Networks report

```
$ wiringcraft report networks
Networks
Network  Type      Device  Description     Address       Station    Port  Connected to  Cable  Pages
-------  --------  ------  --------------  ------------  ---------  ----  ------------  -----  -----
PN1      PROFINET  -A1     PLC             192.168.0.1   plc-line1  X1P1  -K10:X1P1     -W20
PN1      PROFINET  -K10    IO-Link master  192.168.0.10             X1P1  -A1:X1P1      -W20
```

There is one row per link end of each device, in the order the networks and their nodes are
written. A device with no links gets one row. Description comes from the device's data
(`wiringcraft device -K10 --description "IO-Link master"`), and Pages lists the schematic
pages the device is drawn on. In the PDF each row links to the first of those pages.

`--format csv`, `md` and `xlsx` work as for the other reports. See
[docs/cli/report.md](cli/report.md) for the options.

## Warnings

`report` and `pdf` print a warning, and carry on, when a network has:

- a node or link end that is not a device tag,
- a device listed twice, or two nodes with one address or one station name (station names
  are compared without regard to case),
- a link end on a device that is not a node of that network,
- a link from a port to itself, or a port used by more than one link,
- a node that is neither drawn on a page nor listed in the project's devices.

`check` does not look at networks.

## The topology page

When the PDF has the Networks report, each network gets a topology page after it. In the
CLI that is `pdf --report networks` or `pdf --report all` (`all` leaves the report out of a
project without networks). Export PDF in the app always adds them for a project that has
networks.

```
$ wiringcraft pdf out.pdf --report networks
wrote out.pdf (3 pages)
```

Here the 3 pages are the schematic page, the Networks list and the topology page `N1`, titled
`Network PN1 (PROFINET): Line 1 field devices`. The contents report lists the topology pages
as well.

How the page is drawn:

- Each device is a box with its tag, description, address and station. The box links to
  the first page the device is drawn on.
- Each link is drawn once, with its two port names over the line and the cable tag under it.
- The layout follows the links: the first node is on the left and each device it links to
  goes one column further right, so a line of devices runs across the page and a switch
  fans out into a tree.
- A link that closes a loop, such as a ring back to the first device, runs in a lane under
  the boxes. A link between two ports of one device is a small loop over its box.
- Cables from different devices that would run side by side each get their own track, so
  they do not look joined. The cables fanning out of one device share one run.
- Tags, port names and cable tags are never cut. A text too long for its place is drawn in
  smaller type, and past that it is replaced by a key `[n]` and written in full in a legend
  under the drawing.
- The sheet is the size of the first schematic page when the drawing fits on it, else the
  first larger ISO size that fits, else a custom size.

A node or link end that is not a device tag is still drawn as written, with a warning.

The SVG/DXF drawing export (`wiringcraft drawing`) and AutomationML export do not include
networks.
